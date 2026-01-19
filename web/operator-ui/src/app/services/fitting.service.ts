import { Injectable } from '@angular/core';
import { levenbergMarquardt } from 'ml-levenberg-marquardt';

export interface FitInput {
  bins: number[];
  binWidth: number;
  minValue: number;
  fitRangeMin: number;
  fitRangeMax: number;
}

export interface LinearFit {
  slope: number;
  intercept: number;
}

export interface GaussianFitResult {
  // Gaussian parameters
  amplitude: number;
  center: number;
  sigma: number;

  // Background lines
  leftLine: LinearFit;
  rightLine: LinearFit;
  bgLine: LinearFit;

  // Derived values
  fwhm: number;
  netArea: number;

  // Errors (from covariance matrix)
  centerError: number;
  sigmaError: number;
  amplitudeError: number;
  netAreaError: number;

  // Goodness of fit
  chi2: number;
  ndf: number;
}

@Injectable({
  providedIn: 'root',
})
export class FittingService {
  /**
   * Fit a Gaussian peak with linear background
   * Model: y = A * exp(-0.5 * ((x - μ) / σ)²) + m*x + b
   */
  fitGaussian(input: FitInput): GaussianFitResult | null {
    const { bins, binWidth, minValue, fitRangeMin, fitRangeMax } = input;

    // Extract data within fit range
    const { xData, yData } = this.extractRange(bins, binWidth, minValue, fitRangeMin, fitRangeMax);

    if (xData.length < 10) {
      return null;
    }

    // Check if there's any signal
    const maxY = Math.max(...yData);
    if (maxY <= 0) {
      return null;
    }

    // Initial parameter estimates
    const initialParams = this.estimateInitialParams(xData, yData);
    if (!initialParams) {
      return null;
    }

    // Gaussian + linear background model
    // params: [amplitude, center, sigma, bgSlope, bgIntercept]
    const gaussianWithBg = (params: number[]) => (x: number) => {
      const [A, mu, sigma, m, b] = params;
      const gaussian = A * Math.exp(-0.5 * Math.pow((x - mu) / sigma, 2));
      const background = m * x + b;
      return gaussian + background;
    };

    try {
      const result = levenbergMarquardt(
        { x: xData, y: yData },
        gaussianWithBg,
        {
          damping: 1.5,
          initialValues: initialParams,
          gradientDifference: 1e-6,
          maxIterations: 200,
          errorTolerance: 1e-8,
        }
      );

      const [amplitude, center, sigma, bgSlope, bgIntercept] = result.parameterValues;

      // Validate results
      if (sigma <= 0 || amplitude <= 0) {
        return null;
      }

      // Calculate chi-squared
      const { chi2, ndf } = this.calculateChi2(xData, yData, result.parameterValues, gaussianWithBg);

      // Calculate FWHM
      const fwhm = 2.355 * Math.abs(sigma);

      // Calculate net area (Gaussian integral)
      const netArea = amplitude * Math.abs(sigma) * Math.sqrt(2 * Math.PI);

      // Estimate errors from parameter error (simplified)
      const paramErrors = Array.isArray(result.parameterError)
        ? result.parameterError
        : [0, 0, 0, 0, 0];
      const centerError = paramErrors[1] ?? 0;
      const sigmaError = paramErrors[2] ?? 0;
      const amplitudeError = paramErrors[0] ?? 0;

      // Net area error (propagated)
      const netAreaError = netArea * Math.sqrt(
        Math.pow(amplitudeError / amplitude, 2) +
        Math.pow(sigmaError / sigma, 2)
      );

      // Background lines
      const leftLine: LinearFit = { slope: bgSlope, intercept: bgIntercept };
      const rightLine: LinearFit = { slope: bgSlope, intercept: bgIntercept };
      const bgLine: LinearFit = { slope: bgSlope, intercept: bgIntercept };

      return {
        amplitude,
        center,
        sigma: Math.abs(sigma),
        leftLine,
        rightLine,
        bgLine,
        fwhm,
        netArea,
        centerError,
        sigmaError,
        amplitudeError,
        netAreaError,
        chi2,
        ndf,
      };
    } catch {
      console.error('Fitting failed');
      return null;
    }
  }

  /**
   * Fit a linear function to background region
   */
  fitLinearBackground(
    bins: number[],
    binWidth: number,
    minValue: number,
    rangeMin: number,
    rangeMax: number
  ): LinearFit | null {
    const { xData, yData } = this.extractRange(bins, binWidth, minValue, rangeMin, rangeMax);

    if (xData.length < 3) {
      return null;
    }

    // Simple linear regression
    const n = xData.length;
    let sumX = 0, sumY = 0, sumXY = 0, sumX2 = 0;

    for (let i = 0; i < n; i++) {
      sumX += xData[i];
      sumY += yData[i];
      sumXY += xData[i] * yData[i];
      sumX2 += xData[i] * xData[i];
    }

    const denominator = n * sumX2 - sumX * sumX;
    if (Math.abs(denominator) < 1e-10) {
      return null;
    }

    const slope = (n * sumXY - sumX * sumY) / denominator;
    const intercept = (sumY - slope * sumX) / n;

    return { slope, intercept };
  }

  /**
   * Calculate background line connecting left and right edges
   */
  calculateBackgroundLine(
    leftLine: LinearFit,
    rightLine: LinearFit,
    fitRangeMin: number,
    fitRangeMax: number
  ): LinearFit {
    // Value at left edge using left line
    const yLeft = leftLine.slope * fitRangeMin + leftLine.intercept;
    // Value at right edge using right line
    const yRight = rightLine.slope * fitRangeMax + rightLine.intercept;

    // Line connecting these two points
    const slope = (yRight - yLeft) / (fitRangeMax - fitRangeMin);
    const intercept = yLeft - slope * fitRangeMin;

    return { slope, intercept };
  }

  /**
   * Extract x,y data within specified range
   */
  private extractRange(
    bins: number[],
    binWidth: number,
    minValue: number,
    rangeMin: number,
    rangeMax: number
  ): { xData: number[]; yData: number[] } {
    const xData: number[] = [];
    const yData: number[] = [];

    for (let i = 0; i < bins.length; i++) {
      const x = minValue + (i + 0.5) * binWidth;
      if (x >= rangeMin && x <= rangeMax) {
        xData.push(x);
        yData.push(bins[i]);
      }
    }

    return { xData, yData };
  }

  /**
   * Estimate initial parameters for Gaussian fit
   */
  private estimateInitialParams(xData: number[], yData: number[]): number[] | null {
    if (xData.length === 0) return null;

    // Find max for amplitude and center estimate
    let maxY = -Infinity;
    let maxIdx = 0;

    for (let i = 0; i < yData.length; i++) {
      if (yData[i] > maxY) {
        maxY = yData[i];
        maxIdx = i;
      }
    }

    if (maxY <= 0) return null;

    const center = xData[maxIdx];

    // Estimate background from edges
    const nEdge = Math.min(5, Math.floor(xData.length / 10));
    let leftBg = 0, rightBg = 0;
    for (let i = 0; i < nEdge; i++) {
      leftBg += yData[i];
      rightBg += yData[yData.length - 1 - i];
    }
    leftBg /= nEdge;
    rightBg /= nEdge;
    const avgBg = (leftBg + rightBg) / 2;

    // Background slope
    const bgSlope = (rightBg - leftBg) / (xData[xData.length - 1] - xData[0]);
    const bgIntercept = avgBg - bgSlope * center;

    // Amplitude (above background)
    const amplitude = maxY - avgBg;
    if (amplitude <= 0) return null;

    // Estimate sigma from FWHM
    // Find half maximum points
    const halfMax = avgBg + amplitude / 2;
    let leftHalf = center, rightHalf = center;

    for (let i = maxIdx; i >= 0; i--) {
      if (yData[i] < halfMax) {
        leftHalf = xData[i];
        break;
      }
    }

    for (let i = maxIdx; i < yData.length; i++) {
      if (yData[i] < halfMax) {
        rightHalf = xData[i];
        break;
      }
    }

    const fwhmEstimate = rightHalf - leftHalf;
    const sigma = fwhmEstimate / 2.355 || (xData[xData.length - 1] - xData[0]) / 10;

    return [amplitude, center, sigma, bgSlope, bgIntercept];
  }

  /**
   * Calculate chi-squared and degrees of freedom
   */
  private calculateChi2(
    xData: number[],
    yData: number[],
    params: number[],
    model: (params: number[]) => (x: number) => number
  ): { chi2: number; ndf: number } {
    const f = model(params);
    let chi2 = 0;

    for (let i = 0; i < xData.length; i++) {
      const observed = yData[i];
      const expected = f(xData[i]);
      // Use Poisson error estimate: σ² = max(observed, 1)
      const variance = Math.max(observed, 1);
      chi2 += Math.pow(observed - expected, 2) / variance;
    }

    // ndf = number of data points - number of parameters
    const ndf = xData.length - params.length;

    return { chi2, ndf: Math.max(ndf, 1) };
  }
}
